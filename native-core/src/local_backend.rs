// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// LocalUdsBackend — Embedded CDA mode (standalone, direct UDS/DoIP)
//
// Wraps the existing SovdTranslator as a ComponentBackend implementation.
// This is the "local CDA" mode for standalone/testing deployments where no
// external CDA process is running.
//
// In the standard-conformant architecture, this should only be used for:
//   - Development / testing without an external CDA
//   - Embedded deployments where server + CDA are co-located
//
// For production deployments, use SovdHttpBackend pointing to the real CDA.
//
// Gated behind the "local-uds" feature flag.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use async_trait::async_trait;

use crate::translation::SovdTranslator;
use native_interfaces::{
    sovd::{
        SovdBulkDataItem, SovdBulkWriteItem, SovdCapabilities, SovdComponent, SovdComponentConfig,
        SovdDataCatalogEntry, SovdFault, SovdGroup, SovdMode, SovdOperation,
    },
    ComponentBackend, DiagServiceError, DiagnosticSession,
};

/// Local UDS/DoIP backend — wraps `SovdTranslator` behind the `ComponentBackend` trait.
pub struct LocalUdsBackend {
    translator: Arc<SovdTranslator>,
}

impl LocalUdsBackend {
    #[must_use]
    pub fn new(translator: Arc<SovdTranslator>) -> Self {
        Self { translator }
    }

    #[must_use]
    pub fn translator(&self) -> &Arc<SovdTranslator> {
        &self.translator
    }
}

/// Parse a hex DID string (e.g. "F190" or "0xF190") into u16
fn parse_did(data_id: &str) -> Result<u16, DiagServiceError> {
    let hex_str = data_id.trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(hex_str, 16)
        .map_err(|_| DiagServiceError::InvalidRequest(format!("Invalid DID: '{data_id}'")))
}

/// Parse a hex routine ID string into u16
fn parse_routine_id(op_id: &str) -> Result<u16, DiagServiceError> {
    let hex_str = op_id.trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(hex_str, 16)
        .map_err(|_| DiagServiceError::InvalidRequest(format!("Invalid routine ID: '{op_id}'")))
}

#[async_trait]
impl ComponentBackend for LocalUdsBackend {
    fn name(&self) -> &'static str {
        "LocalUdsBackend (embedded CDA)"
    }

    // ── Discovery ───────────────────────────────────────────────────────────

    fn list_components(&self) -> Vec<SovdComponent> {
        self.translator.list_components()
    }

    fn get_component(&self, component_id: &str) -> Option<SovdComponent> {
        self.translator.get_component(component_id)
    }

    // ── Connection lifecycle ────────────────────────────────────────────────

    async fn connect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.translator.connect_component(component_id).await
    }

    async fn disconnect(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.translator.disconnect_component(component_id).await
    }

    // ── Data ────────────────────────────────────────────────────────────────

    fn list_data(&self, component_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        self.translator.list_data_identifiers(component_id)
    }

    async fn read_data(
        &self,
        component_id: &str,
        data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError> {
        let did = parse_did(data_id)?;
        let data = self.translator.read_data(component_id, did).await?;
        Ok(serde_json::json!({
            "componentId": component_id,
            "dataId": data_id,
            "did": format!("0x{did:04X}"),
            "value": hex::encode(&data),
            "rawBytes": data,
        }))
    }

    async fn write_data(
        &self,
        component_id: &str,
        data_id: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        let did = parse_did(data_id)?;
        self.translator.write_data(component_id, did, value).await
    }

    // ── Faults ──────────────────────────────────────────────────────────────

    async fn read_faults(&self, component_id: &str) -> Result<Vec<SovdFault>, DiagServiceError> {
        self.translator.read_faults(component_id).await
    }

    async fn clear_faults(&self, component_id: &str) -> Result<(), DiagServiceError> {
        self.translator.clear_faults(component_id).await
    }

    // ── Operations ──────────────────────────────────────────────────────────

    fn list_operations(&self, component_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
        self.translator.list_operations(component_id)
    }

    async fn execute_operation(
        &self,
        component_id: &str,
        operation_id: &str,
        params: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        let routine_id = parse_routine_id(operation_id)?;
        let result = self
            .translator
            .execute_routine(component_id, routine_id, params)
            .await?;
        Ok(serde_json::json!({ "data": hex::encode(&result) }))
    }

    // ── Capabilities ────────────────────────────────────────────────────────

    fn get_capabilities(&self, component_id: &str) -> Result<SovdCapabilities, DiagServiceError> {
        self.translator.get_capabilities(component_id)
    }

    // ── Mode ────────────────────────────────────────────────────────────────

    fn get_mode(&self, component_id: &str) -> Result<SovdMode, DiagServiceError> {
        self.translator.get_mode(component_id)
    }

    async fn set_mode(&self, component_id: &str, mode: &str) -> Result<(), DiagServiceError> {
        let session = match mode {
            "default" => DiagnosticSession::Default,
            "extended" => DiagnosticSession::Extended,
            "programming" => DiagnosticSession::Programming,
            other => {
                return Err(DiagServiceError::InvalidRequest(format!(
                    "Unknown mode: '{other}'. Use: default, extended, programming"
                )))
            }
        };
        self.translator
            .switch_session(component_id, session)
            .await?;
        Ok(())
    }

    // ── Configuration ───────────────────────────────────────────────────────

    async fn read_config(
        &self,
        component_id: &str,
    ) -> Result<SovdComponentConfig, DiagServiceError> {
        self.translator.read_config(component_id).await
    }

    async fn write_config(
        &self,
        component_id: &str,
        param_name: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.translator
            .write_config(component_id, param_name, value)
            .await
    }

    // ── Bulk Data ───────────────────────────────────────────────────────────

    async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        self.translator.bulk_read(component_id, data_ids).await
    }

    async fn bulk_write(
        &self,
        component_id: &str,
        items: &[SovdBulkWriteItem],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
        self.translator.bulk_write(component_id, items).await
    }

    // ── Groups ──────────────────────────────────────────────────────────────

    fn list_groups(&self) -> Vec<SovdGroup> {
        self.translator.list_groups()
    }

    fn get_group(&self, group_id: &str) -> Option<SovdGroup> {
        self.translator.get_group(group_id)
    }

    // ── Extended Diagnostics ────────────────────────────────────────────────

    async fn io_control(
        &self,
        component_id: &str,
        data_id: &str,
        control: &str,
        value: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        use native_comm_uds::IoControlParameter;

        let did = parse_did(data_id)?;
        let control_param = match control {
            "return_to_ecu" => IoControlParameter::ReturnControlToEcu,
            "reset_to_default" => IoControlParameter::ResetToDefault,
            "freeze" => IoControlParameter::FreezeCurrentState,
            "short_term_adjustment" => IoControlParameter::ShortTermAdjustment,
            other => {
                return Err(DiagServiceError::InvalidRequest(format!(
                    "Invalid IO control: '{other}'"
                )))
            }
        };

        let result = self
            .translator
            .io_control(component_id, did, control_param, value)
            .await?;
        Ok(serde_json::json!({
            "componentId": component_id,
            "did": format!("0x{did:04X}"),
            "control": control,
            "statusRecord": hex::encode(&result),
        }))
    }

    async fn communication_control(
        &self,
        component_id: &str,
        control_type: &str,
        communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        use native_comm_uds::CommControlType;

        let ct = match control_type {
            "enable_rx_and_tx" => CommControlType::EnableRxAndTx,
            "enable_rx_disable_tx" => CommControlType::EnableRxAndDisableTx,
            "disable_rx_enable_tx" => CommControlType::DisableRxAndEnableTx,
            "disable_rx_and_tx" => CommControlType::DisableRxAndTx,
            other => {
                return Err(DiagServiceError::InvalidRequest(format!(
                    "Invalid control_type: '{other}'"
                )))
            }
        };

        self.translator
            .communication_control(component_id, ct, communication_type)
            .await
    }

    async fn dtc_setting(&self, component_id: &str, setting: &str) -> Result<(), DiagServiceError> {
        use native_comm_uds::DtcSettingType;

        let st = match setting {
            "on" => DtcSettingType::On,
            "off" => DtcSettingType::Off,
            other => {
                return Err(DiagServiceError::InvalidRequest(format!(
                    "Invalid DTC setting: '{other}'"
                )))
            }
        };

        self.translator.control_dtc_setting(component_id, st).await
    }

    async fn read_memory(
        &self,
        component_id: &str,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        self.translator
            .read_memory(component_id, address, size)
            .await
    }

    async fn write_memory(
        &self,
        component_id: &str,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        self.translator
            .write_memory(component_id, address, data)
            .await
    }

    async fn flash(
        &self,
        component_id: &str,
        firmware: &[u8],
        memory_address: u32,
    ) -> Result<serde_json::Value, DiagServiceError> {
        let result = self
            .translator
            .flash(component_id, firmware, memory_address)
            .await?;
        Ok(serde_json::to_value(&result).unwrap_or_default())
    }

    fn active_keepalives(&self) -> Vec<String> {
        self.translator.active_keepalives()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translation::{
        ComponentMapping, DataIdentifierDef, GroupDef, OperationDef, TranslationConfig,
    };
    use native_interfaces::ComponentBackend;

    fn make_translator() -> Arc<SovdTranslator> {
        let config = TranslationConfig {
            component_mappings: vec![ComponentMapping {
                sovd_component_id: "hpc".into(),
                sovd_name: "HPC Main".into(),
                doip_target_address: 1,
                doip_source_address: 0x0E00,
                data_identifiers: vec![DataIdentifierDef {
                    did: "F190".into(),
                    name: "VIN".into(),
                    description: None,
                    access: "read-only".into(),
                    unit: None,
                }],
                operations: vec![OperationDef {
                    routine_id: "FF00".into(),
                    name: "Self Test".into(),
                    description: None,
                }],
                group: Some("powertrain".into()),
                features: vec!["faults".into()],
                config_dids: vec![],
            }],
            groups: vec![GroupDef {
                id: "powertrain".into(),
                name: "Powertrain".into(),
                description: Some("Engine group".into()),
            }],
            ..Default::default()
        };
        Arc::new(SovdTranslator::new(config))
    }

    #[test]
    fn name_returns_expected_label() {
        let backend = LocalUdsBackend::new(make_translator());
        assert_eq!(backend.name(), "LocalUdsBackend (embedded CDA)");
    }

    #[test]
    fn translator_accessor_returns_inner() {
        let translator = make_translator();
        let backend = LocalUdsBackend::new(translator.clone());
        // Same Arc
        assert!(Arc::ptr_eq(backend.translator(), &translator));
    }

    #[test]
    fn list_components_delegates_to_translator() {
        let backend = LocalUdsBackend::new(make_translator());
        let components = backend.list_components();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].id, "hpc");
        assert_eq!(components[0].name, "HPC Main");
    }

    #[test]
    fn get_component_delegates_to_translator() {
        let backend = LocalUdsBackend::new(make_translator());
        assert!(backend.get_component("hpc").is_some());
        assert!(backend.get_component("nonexistent").is_none());
    }

    #[test]
    fn handles_component_delegates_to_translator() {
        let backend = LocalUdsBackend::new(make_translator());
        assert!(backend.handles_component("hpc"));
        assert!(!backend.handles_component("unknown"));
    }

    #[test]
    fn list_data_returns_catalog() {
        let backend = LocalUdsBackend::new(make_translator());
        let data = backend.list_data("hpc").unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].name, "VIN");
    }

    #[test]
    fn list_data_not_found_for_unknown() {
        let backend = LocalUdsBackend::new(make_translator());
        assert!(backend.list_data("nonexistent").is_err());
    }

    #[test]
    fn list_operations_returns_ops() {
        let backend = LocalUdsBackend::new(make_translator());
        let ops = backend.list_operations("hpc").unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "Self Test");
    }

    #[test]
    fn get_capabilities_returns_counts() {
        let backend = LocalUdsBackend::new(make_translator());
        let caps = backend.get_capabilities("hpc").unwrap();
        assert_eq!(caps.data_count, 1);
        assert_eq!(caps.operation_count, 1);
    }

    #[test]
    fn list_groups_delegates() {
        let backend = LocalUdsBackend::new(make_translator());
        let groups = backend.list_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "powertrain");
    }

    #[test]
    fn get_group_delegates() {
        let backend = LocalUdsBackend::new(make_translator());
        assert!(backend.get_group("powertrain").is_some());
        assert!(backend.get_group("nonexistent").is_none());
    }

    // ── Helper function tests ───────────────────────────────────────────────

    #[test]
    fn parse_did_valid_hex() {
        assert_eq!(parse_did("F190").unwrap(), 0xF190);
        assert_eq!(parse_did("0xF190").unwrap(), 0xF190);
        assert_eq!(parse_did("0X0200").unwrap(), 0x0200);
    }

    #[test]
    fn parse_did_invalid_returns_error() {
        assert!(parse_did("ZZZZ").is_err());
        assert!(parse_did("").is_err());
    }

    #[test]
    fn parse_routine_id_valid_hex() {
        assert_eq!(parse_routine_id("FF00").unwrap(), 0xFF00);
        assert_eq!(parse_routine_id("0xFF01").unwrap(), 0xFF01);
    }

    #[test]
    fn parse_routine_id_invalid_returns_error() {
        assert!(parse_routine_id("GGGG").is_err());
    }
}
