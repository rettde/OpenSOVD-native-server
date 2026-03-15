// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SOVD ↔ UDS Translation Layer
// Bridges SOVD REST API calls to UDS over DoIP, following CDA patterns.
// Manages ECU connections, session lifecycle, and keepalive.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::info;

use native_comm_doip::{DoipConfig, DoipConnection};
use native_comm_uds::{
    CommControlType, DtcSettingType, IoControlParameter, TesterPresentTask, UdsManager,
};
use native_interfaces::{sovd::*, DiagServiceError, DiagnosticSession};

/// Maps a SOVD component to its UDS/DoIP connection parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMapping {
    pub sovd_component_id: String,
    pub sovd_name: String,
    pub doip_target_address: u16,
    pub doip_source_address: u16,
    /// Available data identifiers (DIDs) for this component
    #[serde(default)]
    pub data_identifiers: Vec<DataIdentifierDef>,
    /// Available operations (routines) for this component
    #[serde(default)]
    pub operations: Vec<OperationDef>,
    /// Group this component belongs to
    #[serde(default)]
    pub group: Option<String>,
    /// Supported capability features
    #[serde(default)]
    pub features: Vec<String>,
    /// Configuration DIDs (readable/writable config parameters)
    #[serde(default)]
    pub config_dids: Vec<DataIdentifierDef>,
}

/// Data identifier definition for component catalogs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataIdentifierDef {
    /// DID hex string, e.g. "F190"
    pub did: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_access")]
    pub access: String,
    #[serde(default)]
    pub unit: Option<String>,
}

fn default_access() -> String {
    "read-only".to_owned()
}

/// Operation (routine) definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationDef {
    /// Routine ID hex string, e.g. "FF00"
    pub routine_id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Group definition for logical component grouping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Translation layer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationConfig {
    pub doip: DoipConfig,
    pub component_mappings: Vec<ComponentMapping>,
    pub tester_present_interval_ms: u64,
    /// Logical component groups
    #[serde(default)]
    pub groups: Vec<GroupDef>,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            doip: DoipConfig::default(),
            component_mappings: vec![],
            tester_present_interval_ms: 2000,
            groups: vec![],
        }
    }
}

/// Core translation service that maps SOVD API calls to UDS over DoIP.
/// Manages active UDS clients, DoIP connections, and TesterPresent keepalive.
pub struct SovdTranslator {
    config: TranslationConfig,
    uds_managers: DashMap<String, Arc<UdsManager>>,
    doip_connections: DashMap<String, Arc<DoipConnection>>,
    tester_present_tasks: DashMap<String, TesterPresentTask>,
}

impl SovdTranslator {
    pub fn new(config: TranslationConfig) -> Self {
        Self {
            config,
            uds_managers: DashMap::new(),
            doip_connections: DashMap::new(),
            tester_present_tasks: DashMap::new(),
        }
    }

    // ── Connection management ───────────────────────────────────────────────

    /// Connect to an ECU component via DoIP/UDS
    #[tracing::instrument(skip(self), fields(component = %component_id))]
    pub async fn connect_component(&self, component_id: &str) -> Result<(), DiagServiceError> {
        let mapping = self
            .config
            .component_mappings
            .iter()
            .find(|m| m.sovd_component_id == component_id)
            .ok_or_else(|| {
                DiagServiceError::NotFound(Some(format!(
                    "Component '{component_id}' not configured"
                )))
            })?
            .clone();

        let mut doip_config = self.config.doip.clone();
        doip_config.source_address = mapping.doip_source_address;

        let doip = Arc::new(DoipConnection::new(
            doip_config,
            mapping.doip_target_address,
        ));

        doip.auto_connect()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("DoIP connect: {e}")))?;

        doip.activate_routing()
            .await
            .map_err(|e| DiagServiceError::SendFailed(format!("Routing activation: {e}")))?;

        let uds = Arc::new(UdsManager::new(doip.clone()));

        // Start TesterPresent keepalive
        let interval = Duration::from_millis(self.config.tester_present_interval_ms);
        let tp_task =
            TesterPresentTask::spawn_for_ecu(component_id.to_owned(), uds.clone(), interval);

        self.doip_connections.insert(component_id.to_owned(), doip);
        self.uds_managers.insert(component_id.to_owned(), uds);
        self.tester_present_tasks
            .insert(component_id.to_owned(), tp_task);

        info!("Connected to component '{component_id}' via DoIP/UDS");
        Ok(())
    }

    /// Disconnect from a component
    #[tracing::instrument(skip(self), fields(component = %component_id))]
    pub async fn disconnect_component(&self, component_id: &str) -> Result<(), DiagServiceError> {
        // Stop TesterPresent first
        if let Some((_, task)) = self.tester_present_tasks.remove(component_id) {
            task.stop();
        }

        if let Some((_, doip)) = self.doip_connections.remove(component_id) {
            doip.disconnect().await;
        }
        self.uds_managers.remove(component_id);

        info!("Disconnected from component '{component_id}'");
        Ok(())
    }

    fn get_uds(&self, component_id: &str) -> Result<Arc<UdsManager>, DiagServiceError> {
        self.uds_managers
            .get(component_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| {
                DiagServiceError::NotFound(Some(format!(
                    "Component '{component_id}' not connected"
                )))
            })
    }

    // ── SOVD → UDS translations ─────────────────────────────────────────────

    /// Read data from a component (SOVD data → UDS 0x22)
    pub async fn read_data(
        &self,
        component_id: &str,
        did: u16,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.read_data_by_identifier(did).await
    }

    /// Write data to a component (SOVD data → UDS 0x2E)
    pub async fn write_data(
        &self,
        component_id: &str,
        did: u16,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.write_data_by_identifier(did, value).await
    }

    /// Read DTCs from a component (SOVD faults → UDS 0x19)
    pub async fn read_faults(
        &self,
        component_id: &str,
    ) -> Result<Vec<SovdFault>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        let dtcs = uds.read_dtc_by_status_mask(0xFF).await?;

        Ok(dtcs
            .iter()
            .map(|dtc| {
                let code = dtc.to_dtc_string();
                SovdFault {
                    id: format!("{component_id}-dtc-{code}"),
                    component_id: component_id.to_owned(),
                    code: code.clone(),
                    display_code: Some(format!("P{code}")),
                    severity: if dtc.is_confirmed() {
                        SovdFaultSeverity::High
                    } else {
                        SovdFaultSeverity::Medium
                    },
                    status: if dtc.is_active() {
                        SovdFaultStatus::Active
                    } else if dtc.is_pending() {
                        SovdFaultStatus::Pending
                    } else {
                        SovdFaultStatus::Passive
                    },
                    name: format!("DTC {code}"),
                    description: Some(format!("Status mask: 0x{:02X}", dtc.status_mask)),
                }
            })
            .collect())
    }

    /// Clear DTCs (SOVD fault clear → UDS 0x14)
    pub async fn clear_faults(&self, component_id: &str) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.clear_dtc(0x00FF_FFFF).await
    }

    /// Execute a routine (SOVD operation → UDS 0x31)
    pub async fn execute_routine(
        &self,
        component_id: &str,
        routine_id: u16,
        params: Option<&[u8]>,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.routine_control_start(routine_id, params).await
    }

    /// Switch diagnostic session (SOVD mode → UDS 0x10)
    pub async fn switch_session(
        &self,
        component_id: &str,
        session: DiagnosticSession,
    ) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.diagnostic_session_control(session).await?;
        Ok(())
    }

    // ── IO Control (SOVD → UDS 0x2F) ──────────────────────────────────────

    /// Control an I/O signal on a component
    pub async fn io_control(
        &self,
        component_id: &str,
        did: u16,
        control_param: IoControlParameter,
        control_option_record: Option<&[u8]>,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.input_output_control(did, control_param, control_option_record)
            .await
    }

    // ── Communication Control (SOVD → UDS 0x28) ─────────────────────────

    /// Control ECU communication (enable/disable Rx/Tx)
    pub async fn communication_control(
        &self,
        component_id: &str,
        control_type: CommControlType,
        communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.communication_control(control_type, communication_type)
            .await
    }

    // ── DTC Setting Control (SOVD → UDS 0x85) ───────────────────────────

    /// Enable or disable DTC recording on a component
    pub async fn control_dtc_setting(
        &self,
        component_id: &str,
        setting_type: DtcSettingType,
    ) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.control_dtc_setting(setting_type).await
    }

    // ── Memory Access (SOVD → UDS 0x23 / 0x3D) ─────────────────────────

    /// Read raw memory from an ECU
    pub async fn read_memory(
        &self,
        component_id: &str,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.read_memory_by_address(address, size).await
    }

    /// Write raw memory to an ECU
    pub async fn write_memory(
        &self,
        component_id: &str,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        uds.write_memory_by_address(address, data).await
    }

    // ── OTA Flash (SOVD → UDS 0x34/0x36/0x37) ─────────────────────────────

    /// Flash firmware to a connected component via UDS
    pub async fn flash(
        &self,
        component_id: &str,
        firmware_data: &[u8],
        memory_address: u32,
    ) -> Result<crate::ota::FlashResult, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        crate::ota::OtaFlashOrchestrator::flash(&uds, component_id, firmware_data, memory_address)
            .await
    }

    // ── Component listing ───────────────────────────────────────────────────

    /// List all configured component mappings as SOVD components
    pub fn list_components(&self) -> Vec<SovdComponent> {
        self.config
            .component_mappings
            .iter()
            .map(|m| {
                let connected = self.uds_managers.contains_key(&m.sovd_component_id);
                SovdComponent {
                    id: m.sovd_component_id.clone(),
                    name: m.sovd_name.clone(),
                    category: "ecu".to_owned(),
                    description: Some(format!("DoIP target 0x{:04X}", m.doip_target_address)),
                    connection_state: if connected {
                        SovdConnectionState::Connected
                    } else {
                        SovdConnectionState::Disconnected
                    },
                }
            })
            .collect()
    }

    /// Get a single component by ID (O(1) lookup instead of iterating all)
    pub fn get_component(&self, component_id: &str) -> Option<SovdComponent> {
        self.config
            .component_mappings
            .iter()
            .find(|m| m.sovd_component_id == component_id)
            .map(|m| {
                let connected = self.uds_managers.contains_key(&m.sovd_component_id);
                SovdComponent {
                    id: m.sovd_component_id.clone(),
                    name: m.sovd_name.clone(),
                    category: "ecu".to_owned(),
                    description: Some(format!("DoIP target 0x{:04X}", m.doip_target_address)),
                    connection_state: if connected {
                        SovdConnectionState::Connected
                    } else {
                        SovdConnectionState::Disconnected
                    },
                }
            })
    }

    /// Get a single group by ID
    pub fn get_group(&self, group_id: &str) -> Option<SovdGroup> {
        self.config
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| {
                let component_ids: Vec<String> = self
                    .config
                    .component_mappings
                    .iter()
                    .filter(|m| m.group.as_deref() == Some(group_id))
                    .map(|m| m.sovd_component_id.clone())
                    .collect();
                SovdGroup {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    description: g.description.clone(),
                    component_ids,
                }
            })
    }

    /// Get active TesterPresent keepalive components
    pub fn active_keepalives(&self) -> Vec<String> {
        self.tester_present_tasks
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    pub fn config(&self) -> &TranslationConfig {
        &self.config
    }

    // ── Data Catalog (SOVD Standard §7.5) ───────────────────────────────────

    /// List available data identifiers for a component
    pub fn list_data_identifiers(
        &self,
        component_id: &str,
    ) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        let mapping = self.find_mapping(component_id)?;
        Ok(mapping
            .data_identifiers
            .iter()
            .map(|d| SovdDataCatalogEntry {
                id: d.did.clone(),
                name: d.name.clone(),
                description: d.description.clone(),
                access: match d.access.as_str() {
                    "read-write" => SovdDataAccess::ReadWrite,
                    "write-only" => SovdDataAccess::WriteOnly,
                    _ => SovdDataAccess::ReadOnly,
                },
                data_type: SovdDataType::Bytes,
                unit: d.unit.clone(),
                did: Some(format!("0x{}", d.did)),
            })
            .collect())
    }

    // ── Operations Listing (SOVD Standard §7.7) ─────────────────────────────

    /// List available operations for a component
    pub fn list_operations(
        &self,
        component_id: &str,
    ) -> Result<Vec<SovdOperation>, DiagServiceError> {
        let mapping = self.find_mapping(component_id)?;
        Ok(mapping
            .operations
            .iter()
            .map(|op| SovdOperation {
                id: op.routine_id.clone(),
                component_id: component_id.to_owned(),
                name: op.name.clone(),
                description: op.description.clone(),
                status: SovdOperationStatus::Idle,
            })
            .collect())
    }

    // ── Capabilities (SOVD Standard §7.3) ───────────────────────────────────

    /// Get capabilities for a component
    pub fn get_capabilities(
        &self,
        component_id: &str,
    ) -> Result<SovdCapabilities, DiagServiceError> {
        let mapping = self.find_mapping(component_id)?;
        let mut features = mapping.features.clone();
        // Auto-detect features from config
        if !mapping.data_identifiers.is_empty() {
            features.push("data".into());
        }
        if !mapping.operations.is_empty() {
            features.push("operations".into());
        }
        if !mapping.config_dids.is_empty() {
            features.push("configuration".into());
        }
        features.sort();
        features.dedup();

        Ok(SovdCapabilities {
            component_id: component_id.to_owned(),
            supported_categories: vec!["ecu".to_owned()],
            data_count: mapping.data_identifiers.len(),
            operation_count: mapping.operations.len(),
            features,
        })
    }

    // ── Groups (SOVD Standard §7.2) ─────────────────────────────────────────

    /// List all groups with their component members
    pub fn list_groups(&self) -> Vec<SovdGroup> {
        self.config
            .groups
            .iter()
            .map(|g| {
                let component_ids: Vec<String> = self
                    .config
                    .component_mappings
                    .iter()
                    .filter(|m| m.group.as_deref() == Some(&g.id))
                    .map(|m| m.sovd_component_id.clone())
                    .collect();
                SovdGroup {
                    id: g.id.clone(),
                    name: g.name.clone(),
                    description: g.description.clone(),
                    component_ids,
                }
            })
            .collect()
    }

    // ── Mode / Session (SOVD Standard §7.6) ─────────────────────────────────

    /// Get current diagnostic mode for a component
    pub fn get_mode(&self, component_id: &str) -> Result<SovdMode, DiagServiceError> {
        self.find_mapping(component_id)?;
        let connected = self.uds_managers.contains_key(component_id);
        Ok(SovdMode {
            component_id: component_id.to_owned(),
            current_mode: if connected {
                "default".into()
            } else {
                "none".into()
            },
            available_modes: vec!["default".into(), "extended".into(), "programming".into()],
        })
    }

    // ── Configuration (SOVD Standard §7.8) ──────────────────────────────────

    /// Read configuration DIDs from a component
    pub async fn read_config(
        &self,
        component_id: &str,
    ) -> Result<SovdComponentConfig, DiagServiceError> {
        let mapping = self.find_mapping(component_id)?;
        let uds = self.get_uds(component_id)?;
        let mut params = serde_json::Map::new();
        for did_def in &mapping.config_dids {
            let did_val = u16::from_str_radix(&did_def.did, 16).map_err(|_| {
                DiagServiceError::InvalidRequest(format!(
                    "Invalid DID hex '{}' in config for '{}'",
                    did_def.did, component_id
                ))
            })?;
            match uds.read_data_by_identifier(did_val).await {
                Ok(data) => {
                    params.insert(
                        did_def.name.clone(),
                        serde_json::Value::String(hex::encode(&data)),
                    );
                }
                Err(e) => {
                    params.insert(
                        did_def.name.clone(),
                        serde_json::Value::String(format!("error: {e}")),
                    );
                }
            }
        }
        Ok(SovdComponentConfig {
            component_id: component_id.to_owned(),
            parameters: serde_json::Value::Object(params),
        })
    }

    /// Write configuration DID to a component
    pub async fn write_config(
        &self,
        component_id: &str,
        did_name: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        let mapping = self.find_mapping(component_id)?;
        let did_def = mapping
            .config_dids
            .iter()
            .find(|d| d.name == did_name)
            .ok_or_else(|| {
                DiagServiceError::NotFound(Some(format!(
                    "Config parameter '{did_name}' not found for '{component_id}'"
                )))
            })?;
        let did_val = u16::from_str_radix(&did_def.did, 16).map_err(|_| {
            DiagServiceError::InvalidRequest(format!(
                "Invalid DID hex '{}' in config for '{}'",
                did_def.did, component_id
            ))
        })?;
        let uds = self.get_uds(component_id)?;
        uds.write_data_by_identifier(did_val, value).await
    }

    // ── Bulk Data (SOVD Standard §7.5.3) ─────────────────────────────────────

    /// Bulk read multiple DIDs
    pub async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        let mut results = Vec::with_capacity(data_ids.len());
        for did_hex in data_ids {
            let did_val = match u16::from_str_radix(did_hex, 16) {
                Ok(v) => v,
                Err(_) => {
                    results.push(SovdBulkDataItem {
                        id: did_hex.clone(),
                        value: None,
                        error: Some(format!("Invalid DID hex: '{did_hex}'")),
                    });
                    continue;
                }
            };
            match uds.read_data_by_identifier(did_val).await {
                Ok(data) => results.push(SovdBulkDataItem {
                    id: did_hex.clone(),
                    value: Some(hex::encode(&data)),
                    error: None,
                }),
                Err(e) => results.push(SovdBulkDataItem {
                    id: did_hex.clone(),
                    value: None,
                    error: Some(e.to_string()),
                }),
            }
        }
        Ok(results)
    }

    /// Bulk write multiple DIDs
    pub async fn bulk_write(
        &self,
        component_id: &str,
        items: &[SovdBulkWriteItem],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        let uds = self.get_uds(component_id)?;
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let did_val = match u16::from_str_radix(&item.id, 16) {
                Ok(v) => v,
                Err(_) => {
                    results.push(SovdBulkDataItem {
                        id: item.id.clone(),
                        value: None,
                        error: Some(format!("Invalid DID hex: '{}'", item.id)),
                    });
                    continue;
                }
            };
            let data = match hex::decode(&item.value) {
                Ok(d) => d,
                Err(e) => {
                    results.push(SovdBulkDataItem {
                        id: item.id.clone(),
                        value: None,
                        error: Some(format!("Invalid hex value: {e}")),
                    });
                    continue;
                }
            };
            match uds.write_data_by_identifier(did_val, &data).await {
                Ok(()) => results.push(SovdBulkDataItem {
                    id: item.id.clone(),
                    value: Some(item.value.clone()),
                    error: None,
                }),
                Err(e) => results.push(SovdBulkDataItem {
                    id: item.id.clone(),
                    value: None,
                    error: Some(e.to_string()),
                }),
            }
        }
        Ok(results)
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    fn find_mapping(&self, component_id: &str) -> Result<ComponentMapping, DiagServiceError> {
        self.config
            .component_mappings
            .iter()
            .find(|m| m.sovd_component_id == component_id)
            .cloned()
            .ok_or_else(|| {
                DiagServiceError::NotFound(Some(format!(
                    "Component '{component_id}' not configured"
                )))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(mappings: Vec<ComponentMapping>) -> TranslationConfig {
        TranslationConfig {
            doip: DoipConfig::default(),
            component_mappings: mappings,
            tester_present_interval_ms: 2000,
            groups: vec![],
        }
    }

    fn test_mapping(id: &str, name: &str, target: u16) -> ComponentMapping {
        ComponentMapping {
            sovd_component_id: id.into(),
            sovd_name: name.into(),
            doip_target_address: target,
            doip_source_address: 0x0E00,
            data_identifiers: vec![],
            operations: vec![],
            group: None,
            features: vec![],
            config_dids: vec![],
        }
    }

    #[test]
    fn default_config_has_empty_mappings() {
        let cfg = TranslationConfig::default();
        assert!(cfg.component_mappings.is_empty());
        assert_eq!(cfg.tester_present_interval_ms, 2000);
    }

    #[test]
    fn list_components_empty() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.list_components().is_empty());
    }

    #[test]
    fn list_components_returns_all_mappings() {
        let mappings = vec![
            test_mapping("hpc", "HPC Main", 0x0001),
            test_mapping("brake", "Brake ECU", 0x0010),
        ];
        let translator = SovdTranslator::new(test_config(mappings));
        let components = translator.list_components();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].id, "hpc");
        assert_eq!(components[0].name, "HPC Main");
        assert_eq!(
            components[0].connection_state,
            SovdConnectionState::Disconnected
        );
        assert_eq!(components[1].id, "brake");
    }

    #[test]
    fn list_components_category_is_ecu() {
        let translator = SovdTranslator::new(test_config(vec![test_mapping("x", "X", 1)]));
        let comps = translator.list_components();
        assert_eq!(comps[0].category, "ecu");
    }

    #[test]
    fn list_components_description_contains_target_address() {
        let translator = SovdTranslator::new(test_config(vec![test_mapping("x", "X", 0x00FF)]));
        let comps = translator.list_components();
        let desc = comps[0].description.as_ref().unwrap();
        assert!(desc.contains("00FF"));
    }

    #[test]
    fn active_keepalives_empty_when_no_connections() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.active_keepalives().is_empty());
    }

    #[test]
    fn config_accessor() {
        let cfg = test_config(vec![test_mapping("a", "A", 1)]);
        let translator = SovdTranslator::new(cfg);
        assert_eq!(translator.config().component_mappings.len(), 1);
        assert_eq!(
            translator.config().component_mappings[0].sovd_component_id,
            "a"
        );
    }

    fn mapping_with_dids_and_ops() -> ComponentMapping {
        ComponentMapping {
            sovd_component_id: "hpc".into(),
            sovd_name: "HPC Main".into(),
            doip_target_address: 0x0001,
            doip_source_address: 0x0E00,
            data_identifiers: vec![
                DataIdentifierDef {
                    did: "F190".into(),
                    name: "VIN".into(),
                    description: None,
                    access: "read-only".into(),
                    unit: None,
                },
                DataIdentifierDef {
                    did: "0200".into(),
                    name: "Voltage".into(),
                    description: Some("System voltage".into()),
                    access: "read-only".into(),
                    unit: Some("V".into()),
                },
            ],
            operations: vec![OperationDef {
                routine_id: "FF00".into(),
                name: "Self Test".into(),
                description: Some("ECU self-test".into()),
            }],
            group: Some("powertrain".into()),
            features: vec!["faults".into(), "ota".into()],
            config_dids: vec![DataIdentifierDef {
                did: "F100".into(),
                name: "Coding".into(),
                description: None,
                access: "read-write".into(),
                unit: None,
            }],
        }
    }

    #[test]
    fn list_data_identifiers_returns_dids() {
        let translator = SovdTranslator::new(test_config(vec![mapping_with_dids_and_ops()]));
        let entries = translator.list_data_identifiers("hpc").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "F190");
        assert_eq!(entries[0].name, "VIN");
        assert_eq!(entries[1].unit, Some("V".into()));
    }

    #[test]
    fn list_data_identifiers_unknown_component_errors() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.list_data_identifiers("unknown").is_err());
    }

    #[test]
    fn list_data_identifiers_empty_when_no_dids() {
        let translator = SovdTranslator::new(test_config(vec![test_mapping("x", "X", 1)]));
        let entries = translator.list_data_identifiers("x").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn list_operations_returns_ops() {
        let translator = SovdTranslator::new(test_config(vec![mapping_with_dids_and_ops()]));
        let ops = translator.list_operations("hpc").unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "Self Test");
        assert!(ops[0].description.is_some());
    }

    #[test]
    fn list_operations_unknown_component_errors() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.list_operations("unknown").is_err());
    }

    #[test]
    fn get_capabilities_returns_features_and_counts() {
        let translator = SovdTranslator::new(test_config(vec![mapping_with_dids_and_ops()]));
        let caps = translator.get_capabilities("hpc").unwrap();
        assert_eq!(caps.component_id, "hpc");
        assert_eq!(caps.data_count, 2);
        assert_eq!(caps.operation_count, 1);
        assert!(caps.features.contains(&"faults".to_string()));
        assert!(caps.features.contains(&"ota".to_string()));
    }

    #[test]
    fn get_capabilities_unknown_component_errors() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.get_capabilities("unknown").is_err());
    }

    #[test]
    fn list_groups_returns_configured_groups() {
        let mut cfg = test_config(vec![mapping_with_dids_and_ops()]);
        cfg.groups = vec![GroupDef {
            id: "powertrain".into(),
            name: "Powertrain".into(),
            description: Some("Engine and battery".into()),
        }];
        let translator = SovdTranslator::new(cfg);
        let groups = translator.list_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "powertrain");
        assert_eq!(groups[0].name, "Powertrain");
        // Should include hpc since its group == "powertrain"
        assert!(groups[0].component_ids.contains(&"hpc".to_string()));
    }

    #[test]
    fn list_groups_empty_when_no_groups_configured() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.list_groups().is_empty());
    }

    #[test]
    fn get_mode_returns_none_when_disconnected() {
        let translator = SovdTranslator::new(test_config(vec![mapping_with_dids_and_ops()]));
        let mode = translator.get_mode("hpc").unwrap();
        assert_eq!(mode.component_id, "hpc");
        // Not connected → current mode is "none"
        assert_eq!(mode.current_mode, "none");
        assert!(mode.available_modes.contains(&"default".to_string()));
        assert!(mode.available_modes.contains(&"extended".to_string()));
        assert!(mode.available_modes.contains(&"programming".to_string()));
    }

    #[test]
    fn get_mode_unknown_component_errors() {
        let translator = SovdTranslator::new(test_config(vec![]));
        assert!(translator.get_mode("unknown").is_err());
    }

    #[test]
    fn find_mapping_returns_correct_mapping() {
        let mappings = vec![test_mapping("a", "A", 1), test_mapping("b", "B", 2)];
        let translator = SovdTranslator::new(test_config(mappings));
        let m = translator.find_mapping("b").unwrap();
        assert_eq!(m.sovd_component_id, "b");
        assert_eq!(m.doip_target_address, 2);
    }

    #[test]
    fn find_mapping_not_found_error() {
        let translator = SovdTranslator::new(test_config(vec![]));
        let err = translator.find_mapping("missing");
        assert!(err.is_err());
    }
}
